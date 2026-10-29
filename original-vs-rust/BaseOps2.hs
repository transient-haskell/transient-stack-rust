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
  run "O4 <**: an event of mb runs ma again" $ do
    ev <- newEVar
    n <- liftIO $ newIORef (0 :: Int)
    (do r <- (do k <- liftIO $ atomicModifyIORef n (\a -> (a + 1, a + 1)); say ("  ma executed " ++ show k); return k) <** readEVar ev
        say ("  r " ++ show (r :: Int))
        empty)
      <|> (do liftIO (threadDelay 20000); writeEVar ev (); writeEVar ev (); cleanEVar ev; empty)

  run "T1c addThreads ANTES de lanzar: pico de concurrencia" $ do
    cur <- liftIO $ newIORef (0 :: Int)
    peak <- liftIO $ newIORef (0 :: Int)
    x <- threads 1 $ addThreads 3 >> choose [1 .. 4 :: Int]
    c <- liftIO $ atomicModifyIORef cur (\a -> (a + 1, a + 1))
    liftIO $ atomicModifyIORef peak (\p -> (max p c, ()))
    liftIO $ threadDelay 60000
    liftIO $ atomicModifyIORef cur (\a -> (a - 1, ()))
    p <- liftIO $ readIORef peak
    say ("x " ++ show x ++ " pico " ++ show p)

  run "T1d addThreads 2 with limit 3: does not lower the limit" $ do
    cur <- liftIO $ newIORef (0 :: Int)
    peak <- liftIO $ newIORef (0 :: Int)
    x <- threads 3 $ addThreads 2 >> choose [1 .. 5 :: Int]
    c <- liftIO $ atomicModifyIORef cur (\a -> (a + 1, a + 1))
    liftIO $ atomicModifyIORef peak (\p -> (max p c, ()))
    liftIO $ threadDelay 60000
    liftIO $ atomicModifyIORef cur (\a -> (a - 1, ()))
    p <- liftIO $ readIORef peak
    say ("x " ++ show x ++ " pico " ++ show p)

  run "T4 onWaitThreads: when the threads are stopped waiting for events" $ do
    onWaitThreads (\_ -> say "  WAITTHREADS")
    x <- choose [1, 2, 3 :: Int]
    liftIO $ threadDelay (x * 40000)
    say ("work " ++ show x)
