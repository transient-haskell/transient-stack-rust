{-# LANGUAGE ScopedTypeVariables #-}
import Transient.Base
import Transient.Internals
import Transient.Indeterminism
import Control.Monad
import Control.Applicative
import Data.Typeable
import Control.Monad.IO.Class
import Control.Concurrent
import Control.Exception hiding (onException, catch, try)
import qualified System.Timeout as T
import Data.IORef
import qualified Data.Map as M

say :: String -> TransIO ()
say = liftIO . putStrLn

run :: String -> TransIO () -> IO ()
run name t = do
  putStrLn ("--- " ++ name)
  r <- T.timeout 2000000 (keep' t)
  case r of
    Nothing -> putStrLn "   (keep' returned nothing in 2 s)"
    Just rs -> putStrLn ("   keep' returns " ++ show (length rs) ++ " results")

main = do
  -- ---------- estado
  run "S1 modifyData' with and without previous data" $ do
    r1 <- modifyData' (\h -> h ++ " world") "hello new"
    v1 <- getSData :: TransIO String
    say ("no previous: returns " ++ show r1 ++ ", stored " ++ show v1)
    setData "hello"
    r2 <- modifyData' (\h -> h ++ " world") "hello new"
    v2 <- getSData :: TransIO String
    say ("with previous: returns " ++ show r2 ++ ", stored " ++ show v2)

  run "S2 modifyData: Nothing borra, Just actualiza" $ do
    setData (5 :: Int)
    modifyData (fmap (+ (1 :: Int)))
    a <- getSData :: TransIO Int
    modifyData (\(_ :: Maybe Int) -> Nothing)
    b <- (Just <$> (getSData :: TransIO Int)) <|> return Nothing
    modifyData (\(m :: Maybe Int) -> Just (maybe 100 id m))
    c <- getSData :: TransIO Int
    say (show (a, b, c))

  run "S3 try: restores the state on failure; sandbox: always" $ do
    setData "base"
    (try (setData "changed" >> empty)) <|> return ()
    a <- getSData :: TransIO String
    b <- (sandbox (setData "inside" >> return ()) >> getSData :: TransIO String)
    c <- (try (setData "ok" >> return ()) >> getSData :: TransIO String)
    say (show (a, b, c))

  run "S4 indexed data: pure (per branch) and mutable (shared)" $ do
    setIndexData (1 :: Int) "one"
    setIndexData (2 :: Int) "two"
    a <- getIndexData (1 :: Int) :: TransIO (Maybe String)
    b <- getIndexData (3 :: Int) :: TransIO (Maybe String)
    setIndexRData (7 :: Int) "seven"
    withIndexRData (7 :: Int) (++ "!") "x"
    withIndexRData (8 :: Int) (++ "!") "eight"
    abduce
    setIndexData (1 :: Int) "changed in the branch"
    setIndexRData (9 :: Int) "nine"
    r7 <- getIndexRData (7 :: Int) :: TransIO (Maybe String)
    r8 <- getIndexRData (8 :: Int) :: TransIO (Maybe String)
    r9 <- getIndexRData (9 :: Int) :: TransIO (Maybe String)
    say (show (a, b, r7, r8, r9))

  -- ---------- operadores
  run "O1 **>: runs the second even if the first is empty" $ do
    r <- (say "a" >> empty) **> (say "b" >> return (2 :: Int))
    say ("r " ++ show r)

  run "O2 <***: mb after each thread of ma, before what follows" $ do
    x <- choose [1, 2, 3 :: Int] <*** say "  mb"
    say ("x " ++ show x)

  run "O2b <*** with synchronous ma" $ do
    x <- (say "ma" >> return (1 :: Int)) <*** say "  mb"
    say ("x " ++ show x)

  run "O3 <->: both continuations run in this thread" $ do
    r <- (say "one" >> return (1 :: Int)) <-> (say "two" >> return 2)
    th <- liftIO myThreadId
    say ("r " ++ show r)

  run "O3b <|> frente a <->" $ do
    r <- (say "one" >> return (1 :: Int)) <|> (say "two" >> return 2)
    say ("r " ++ show r)

  -- ---------- threads
  run "T1 threads 1 y addThreads: pico de concurrencia" $ do
    cur <- liftIO $ newIORef (0 :: Int)
    peak <- liftIO $ newIORef (0 :: Int)
    x <- threads 1 $ choose [1 .. 4 :: Int]
    addThreads 3
    c <- liftIO $ atomicModifyIORef cur (\a -> (a + 1, a + 1))
    liftIO $ atomicModifyIORef peak (\p -> (max p c, ()))
    liftIO $ threadDelay 60000
    liftIO $ atomicModifyIORef cur (\a -> (a - 1, ()))
    p <- liftIO $ readIORef peak
    say ("x " ++ show x ++ " pico " ++ show p)

  run "T1b threads 1 without addThreads" $ do
    cur <- liftIO $ newIORef (0 :: Int)
    peak <- liftIO $ newIORef (0 :: Int)
    x <- threads 1 $ choose [1 .. 4 :: Int]
    c <- liftIO $ atomicModifyIORef cur (\a -> (a + 1, a + 1))
    liftIO $ atomicModifyIORef peak (\p -> (max p c, ()))
    liftIO $ threadDelay 60000
    liftIO $ atomicModifyIORef cur (\a -> (a - 1, ()))
    p <- liftIO $ readIORef peak
    say ("x " ++ show x ++ " pico " ++ show p)

  run "T2 hookedThreads inside freeThreads: keep' waits" $ do
    freeThreads $ hookedThreads $ async (threadDelay 300000)
    say "after hooked"

  run "T2b freeThreads: keep' does not wait" $ do
    freeThreads $ async (threadDelay 300000)
    say "after free"

  run "T3 onFinish: with a live listener it does not run; onWaitThreads does" $ do
    onFinish (\_ -> say "  FINISH")
    onWaitThreads (\_ -> say "  WAITTHREADS")
    x <- choose [1, 2 :: Int]
    liftIO $ threadDelay (x * 30000)
    say ("work " ++ show x)
    -- a listener that keeps the tree alive
    (do _ <- getMailbox :: TransIO Int; return ()) <|> return ()

  run "T3b onWaitThreads without listeners" $ do
    onWaitThreads (\_ -> say "  WAITTHREADS")
    x <- choose [1, 2 :: Int]
    liftIO $ threadDelay (x * 30000)
    say ("work " ++ show x)

  run "OC openClose: closes when the threads finish" $ do
    r <- openClose (say "  open" >> return (7 :: Int)) (\v -> say ("  close " ++ show v))
    x <- choose [1, 2 :: Int]
    liftIO $ threadDelay (x * 30000)
    say ("uses " ++ show (r, x))

  -- ---------- localBack with abduce and an alternative
  run "L1 localBack with abduce and an alternative: does it restore in the calling thread?" $ do
    (return ()) `onUndo` (say "  outer")
    (localBack () (do (return ()) `onUndo` (say "  inner"); abduce)) <|> undo
