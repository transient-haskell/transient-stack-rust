{-# LANGUAGE ScopedTypeVariables #-}
import Transient.Base
import Transient.Internals
import Transient.Indeterminism
import Control.Monad
import Control.Applicative
import Data.Typeable
import Control.Monad.IO.Class
import Control.Concurrent
import Control.Exception hiding (onException, catch)
import qualified System.Timeout as T
import Data.IORef

say :: String -> TransIO ()
say = liftIO . putStrLn

run :: String -> TransIO () -> IO ()
run name t = do
  putStrLn ("--- " ++ name)
  r <- T.timeout 3000000 (keep' t)
  case r of
    Nothing -> putStrLn "   (keep' returned nothing in 3 s)"
    Just rs -> putStrLn ("   keep' returns " ++ show (length rs) ++ " results")

main = do
  run "O2 oneThread: does the producer keep generating events?" $ do
    n <- liftIO $ newIORef (0 :: Int)
    x <- oneThread $ waitEvents (do threadDelay 20000
                                    k <- atomicModifyIORef n (\a -> (a+1, a+1))
                                    putStrLn ("  produce " ++ show k)
                                    return k)
    say ("  event " ++ show x)
    async (threadDelay 50000)
    say ("  process " ++ show x)

  run "O3 without oneThread: the same, for comparison" $ do
    n <- liftIO $ newIORef (0 :: Int)
    x <- waitEvents (do threadDelay 20000
                        k <- atomicModifyIORef n (\a -> (a+1, a+1))
                        putStrLn ("  produce " ++ show k)
                        return k)
    when (x > 4) $ empty
    say ("  event " ++ show x)
    async (threadDelay 50000)
    say ("  process " ++ show x)
